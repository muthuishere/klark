# klark handover

Read this first if you are picking up klark, whether you are a person or an agent.
State as of 2026-11-06, after PR #1 was merged into `master` (merge commit `95b184c`).

## What klark is

Two things in one repository:

1. **klark**: a terminal runtime for coding agents (Rust, one binary), forked
   from herdr 0.9.5 and renamed. It has no self-update and makes no hosted
   calls.
2. **klark hub**: a **separate web server** (C# Blazor Server, .NET 9, SQLite,
   Docker) in `hub/`. Deploy it anywhere. Any number of klark machines connect
   to it with `klark expose`, and many people sign in to watch and answer every
   machine's agents. It is built for 100+ machines.

```
klark machine ─┐  klark expose --hub ws://<hub>:8080/agent --token …
klark machine ─┼──────────────────────────────▶  hub (Docker)  ◀── browsers
      …        ┘   machines dial out; the hub pulls; nothing is pushed unasked
```

herdr-expose (`muthuishere/herdr-expose`) is a **separate product** that stays
as it is. The hub follows its ideas but shares no code with it (ADR 0008).

## State (2026-11-06, branch `klark-messaging`)

Both halves now build and run on a Mac:
- `cargo build --release` works (Rust + Zig 0.16), and `just test` (nextest) passes: 3946 tests.
- The hub builds on .NET 10, and its Docker image builds and serves requests.
- Agents can message each other (ADR 0009) across sessions and across machines
  through the hub. This was proven by `tests/msg_sessions.rs` and by a scripted
  two-machine run against the real hub.

Still unverified: sign-in against a real LDAP server (S-28), and clicking
through the pane view in a browser (S-03).

## First steps for whoever picks this up

1. **Hub:**
   - LDAP (the default):
     `KLARK_HUB__AUTH__LDAP__HOST=<ldap> ... docker compose up -d --build`
   - Built-in mock LDAP instead:
     `KLARK_HUB_AUTH_MODE=MockLdap docker compose up -d --build`
   - See `hub/README.md`.
2. **klark:** `cargo build --release`, then `just test`.
3. **Link a machine:**
   `klark expose --hub ws://<hub>:8080/agent --token <token>`.
4. **Messaging:**
   - Find agents: `klark msg ls`.
   - Send: `klark msg send <machine>/<session>/<agent> "..."`.
   - The receiving agent answers with `klark msg reply "..."`.
   - People sign in with `klark msg login --hub <url> --user <name>`.
5. **Try a real directory** (S-28), and **click through the browser** (S-03).
6. **Measure the real hub** with `klark-loadtest --machines 100` (S-12, E-06).
7. **Delete the old workflow files** in `.github/workflows/` by hand (S-04).

## Where things are

| path | what |
|---|---|
| `src/link_cmd.rs` | `klark expose` and `klark expose url` |
| `crates/klark-link/src/protocol.rs` | wire protocol v1: every message, the read-only call allow-list |
| `crates/klark-link/src/lib.rs` | connection loop, binary frames, 50 ms merging, reconnect jitter, `pane_url` |
| `crates/klark-link/src/upstream.rs` | talks to the local klark socket API and `terminal session observe` |
| `crates/klark-link/src/bin/klark-loadtest.rs` | simulates N machines |
| `hub/Program.cs` | endpoints: `/agent` (machines), login/logout, rate limit, DI |
| `hub/Services/MachineRegistry.cs` | per-machine connection, lazy tree pulls, per-viewer queues |
| `hub/Services/HubStore.cs` | SQLite: `hub.db` + `machines/<id>.db`, 10 MB cap |
| `hub/Services/Directory.cs` | sign-in: MockLdap / Ldap / Local |
| `hub/Services/HubAccess.cs` | who sees and who may type |
| `hub/Components/Pages/` | Home (machines), PaneView (live terminal, transcript), Go (share links), Login |
| `skills/klark-share/SKILL.md` | "share this" → `klark expose url` |
| `src/msg_cmd.rs`, `skills/klark-msg/SKILL.md` | `klark msg`: agents messaging agents (ADR 0009) |
| `hub/Services/Messaging.cs`, `ApiAuth.cs` | hub message routing, JWTs and machine-token API auth |
| `experiments/` | `mockhub.py` and `drive.py` behind the experiments |

## Decisions (do not undo without a new ADR)

| ADR | decision |
|---|---|
| 0001 | rename, no phoning home, overall architecture |
| 0002 | **always pull**: a machine sends only `hello` unasked; the hub pulls trees; output flows only while a pane is viewed |
| 0003 | SQLite, one file per machine, 10 MB cap |
| 0004 | sign-in (mock LDAP default, real LDAP, local), machine and fleet tokens, groups |
| 0005 | **plain HTTP** (`ws://`); TLS only via a proxy in front, if ever |
| 0006 | budget for 100+ machines |
| 0007 | **no CI**; build and test by hand |
| 0008 | the hub is a separate server; herdr-expose is a separate product |
| 0009 | agents message agents (CLI → hub HTTP → link `deliver`); user JWTs; **LDAP is the default sign-in**, MockLdap opt-in |

Rules for agents working here are in `AGENTS.md`. The contribution flow is in
`CONTRIBUTING.md`.

## Backlog

`docs/STATUS.md` is the single list. Besides the must-do items above:

- **Hub features:**
  - S-08 admin page for tokens and users;
  - S-24 guest links (no account, one pane, expiring, QR code);
  - S-26 disconnect or revoke a machine;
  - S-19 / S-21 search and blocked-pane notifications.
- **klark side:**
  - S-15 build the tree from `events.subscribe` instead of polling snapshots;
  - S-16 Windows `klark expose`;
  - S-17 remove the dead updater code;
  - S-27 `klark expose` as an autostart service and `doctor`.
- **Experiments still to run:** E-05..E-10 (adaptive merging, real hub
  resources, zstd, SQLite load, phone rendering, reconnect storm).

## Known risks

- **Plain HTTP:** passwords and tokens cross the network in clear text. Run
  the hub on a trusted LAN or VPN.
- **MockLdap** (opt-in) has demo passwords (admin/admin…). Use it only for trials.
- **Untested rename:** the herdr → klark rename touched ~300 files mechanically
  and moves `~/.config/herdr` to `~/.config/klark` on first run. Check this on
  a real install.
- **Windows ConPTY names** were renamed in the vendored portable-pty patches,
  but this has not been run on Windows.
- **The bitnami OpenLDAP image** used by the compose `ldap` profile may have
  moved registries. If it fails to pull, swap in another OpenLDAP image.
