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
   to it with `klark link`, and many people sign in to watch and answer every
   machine's agents. It is built for 100+ machines.

```
klark machine ─┐  klark link --hub ws://<hub>:8080/agent --token …
klark machine ─┼──────────────────────────────▶  hub (Docker)  ◀── browsers
      …        ┘   machines dial out; the hub pulls; nothing is pushed unasked
```

herdr-expose (`muthuishere/herdr-expose`) is a **separate product** that stays
as it is. The hub follows its ideas but shares no code with it (ADR 0008).

## ⚠️ The most important fact

**The klark binary and the hub have never been compiled or run.** The
authoring sessions could not download Zig 0.16 or the .NET SDK, because the
cloud network blocked them. What is verified:

| part | verified by |
|---|---|
| `crates/klark-link` (link protocol, wire encoding, jitter, URLs) | `cargo test -p klark-link`: 13 unit tests + 1 end-to-end test, all pass |
| 100-machine behavior | `klark-loadtest` against a Python stand-in hub (`experiments/`), see `docs/experiments/E-01..E-04` |
| maintenance scripts | 62 Python tests pass |
| `src/` changes (rename, `klark link`, disabled updater) | **not compiled** |
| `hub/` | **not compiled**; it was written without a compiler, so expect errors on the first build |

## First steps for whoever picks this up

1. **Build the hub:** `cd hub && docker compose up -d --build`, or
   `dotnet build`. Fix the compile errors. (STATUS S-02)
2. **Build klark:** install Zig 0.16 and set `ZIG=/path/to/zig`, then run
   `cargo build --release` and `just test`. Fix any fallout from the herdr →
   klark rename. (S-01)
3. **End to end:**
   1. Sign in to `http://localhost:8080` as `admin` / `admin` (the default mock
      LDAP mode).
   2. Configure a machine token (see `hub/README.md`).
   3. On a machine, run
      `klark link --hub ws://localhost:8080/agent --token <token>`.
   4. Open a pane, type a reply, and check that it lands in the terminal. (S-03)
4. **Measure the real hub** with `klark-loadtest --machines 100` and a fleet
   token. Record CPU and RAM. (S-12, E-06)
5. **GitHub cleanup (by hand):** delete the 10 old herdr workflow files in
   `.github/workflows/`. There is no CI by decision (ADR 0007), and agent
   sessions cannot touch that folder. (S-04)

## Where things are

| path | what |
|---|---|
| `src/link_cmd.rs` | `klark link` and `klark link url` |
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
| `skills/klark-share/SKILL.md` | "share this" → `klark link url` |
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

Rules for agents working here are in `AGENTS.md`. The contribution flow is in
`CONTRIBUTING.md`.

## Backlog

`docs/STATUS.md` is the single list. Besides the must-do items above:

- **Hub features:**
  - S-08 admin page for tokens and users;
  - S-24 guest links (no account, one pane, expiring, QR code);
  - S-25 agent-to-agent messaging through the hub (write an ADR first);
  - S-26 disconnect or revoke a machine;
  - S-19 / S-21 search and blocked-pane notifications.
- **klark side:**
  - S-15 build the tree from `events.subscribe` instead of polling snapshots;
  - S-16 Windows `klark link`;
  - S-17 remove the dead updater code;
  - S-27 `klark link` as an autostart service and `doctor`.
- **Experiments still to run:** E-05..E-10 (adaptive merging, real hub
  resources, zstd, SQLite load, phone rendering, reconnect storm).

## Known risks

- **Plain HTTP:** passwords and tokens cross the network in clear text. Run
  the hub on a trusted LAN or VPN.
- **Mock LDAP is the default sign-in mode,** with demo passwords
  (admin/admin…). Switch `Hub:Auth:Mode` before exposing the hub.
- **Untested rename:** the herdr → klark rename touched ~300 files mechanically
  and moves `~/.config/herdr` to `~/.config/klark` on first run. Check this on
  a real install.
- **Windows ConPTY names** were renamed in the vendored portable-pty patches,
  but this has not been run on Windows.
- **The bitnami OpenLDAP image** used by the compose `ldap` profile may have
  moved registries. If it fails to pull, swap in another OpenLDAP image.
