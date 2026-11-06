# klark

klark is a terminal runtime for coding agents with a built-in link to a shared
web hub. It is a fork of herdr (Apache-2.0) that has gone its own way: no
self-update, no hosted website, no release channels.

This file is for any agent (or person) working in this repository.

## Map

| path | what | language |
|---|---|---|
| `src/` | klark itself: server, TUI client, panes, agent detection, socket API | Rust |
| `crates/klark-link` | the machine side of the hub link, plus `klark-loadtest` | Rust |
| `crates/ghostty-vt` | bindings to the vendored libghostty-vt terminal core | Rust + Zig |
| `hub/` | the hub: ASP.NET Core Blazor Server, SQLite, Docker | C# (.NET 9) |
| `api/klark-api.schema.json` | socket API schema | JSON |
| `docs/STATUS.md` | **the backlog**: done, must do, features (IDs like S-05) | |
| `docs/adr/` | decisions; `docs/experiments/` measured experiments | |
| `experiments/` | scripts behind the experiments | Python |

Before starting work, read `docs/STATUS.md` and the ADRs that touch the area
you are changing.

## Decisions you must not undo without a new ADR

- **No phoning home.** klark makes no network calls of its own: no update
  checks, announcements or remote catalogs (ADR 0001 D1). `klark link` talks
  only to the hub the user configured.
- **Always pull** (ADR 0002). A linked machine sends nothing unasked except
  `hello`. The hub pulls trees with `get_tree`. Pane output flows only between
  `subscribe` and `unsubscribe`. Never add a message a machine sends on its own.
- **The hub can only read, type and send keys.** `call` is limited to the
  read-only allow-list in `crates/klark-link/src/protocol.rs`. Never widen it
  to methods that create, close or run things.
- **Plain HTTP** for the hub (ADR 0005). Don't add HTTPS redirects or HSTS.
  TLS is a proxy in front, if ever.
- **SQLite only for the hub**, one file per machine, capped at 10 MB (ADR 0003).
- **No CI** (ADR 0007). Verification is local; see Testing.

## klark core (Rust, `src/`)

### Principles

- **State is separated from runtime.** `AppState` is pure data, testable without PTYs or async. `PaneState` is separate from `PaneRuntime`. Workspace logic doesn't need real terminals.
- **Render is pure.** `compute_view()` handles geometry and mutations. `render()` takes `&AppState` and only draws. Never mutate state during render.
- **No god objects.** If a module is doing too many things, split it. `app/` is already split into state, actions, and input. Keep it that way.
- **Platform code is isolated.** OS-specific behavior lives in the matching `src/platform/<os>.rs` file, with only shared traits, types, wrappers, and testable contracts in `src/platform/mod.rs`. Core modules don't have `#[cfg(target_os)]`.
- **Detection is decoupled.** The detector reads a screen snapshot, never touches the parser or viewport state.
- **UI patterns are reused.** klark is a mouse-first TUI. New dialogs, settings and flows follow the existing modal/screen structure, affordances and close actions instead of one-off screens.

### Multiplicative performance paths

Treat work reachable from view computation, rendering, background-pane resizing,
PTY parsing, detection, and client frame fanout as multiplicative. Before adding
work, identify its frequency and cardinality: per byte, event, or render × panes,
tabs, or workspaces × attached clients.

Inside pane-scaled render and layout loops:

- Use narrow terminal-state accessors. Do not collect aggregate input state,
  format terminal snapshots, inspect process trees, perform filesystem I/O, or
  allocate when one scalar fact is enough.
- Keep terminal-core lock duration minimal.
- Preserve hidden-source and retained-render early exits. Hidden panes still
  parse output, but their output must not trigger presentation work merely to
  keep terminal or detection state current.
- When a change adds or widens work in one of these loops, profile fixed geometry
  with 1 and at least 15 populated panes and report the scaling delta
  (`just bench-render-scale`).

Prefer deterministic operation or architecture tests to wall-clock limits.
Benchmarks are supporting evidence, not substitutes for behavioral coverage.

### Runtime/client boundary

klark is moving toward a server-owned runtime protocol with the TUI as one
client, and the hub link as another. New work must not deepen server/TUI coupling.

Before adding state, API fields, events, commands, or socket messages, classify the feature:

- Shared runtime/session fact (pane/agent metadata, process and terminal state,
  events): belongs in server state, exposed through the JSON API/event path.
  This is also what the hub can reach.
- TUI presentation state (sidebar layout, colors, selection, modals, mouse and
  viewport state): belongs only in the TUI client.

Don't add shared behavior that only works through the private TUI client socket. Use neutral API names, not UI names like sidebar, row, card, or widget.

### Wire compatibility

- **Socket API** (`api/klark-api.schema.json`): new JSON fields must be
  optional or defaulted; don't change the meaning or required parameters of an
  existing method. Add a new method instead. `klark-link` and the hub depend
  on `session.snapshot`, `pane.read`, `agent.read`, `pane.send_text`,
  `pane.send_keys` and `klark terminal session observe`.
- **TUI client endpoint**: named core codecs are immutable. Frozen endpoint
  fixtures, bincode digests, wire-tag tests and
  `tests/fixtures/endpoint-method-shapes-v1.json` are contracts. Never update
  one to bless a wire change; add a new codec or method. Treat enums reachable
  from a frozen codec as append-closed.
- **Server/client protocol** (`src/protocol/wire.rs::PROTOCOL_VERSION`): bump
  it on an incompatible wire change and update the hardcoded expectations in tests.
- **Hub link protocol** (`klark_link::protocol::PROTOCOL_VERSION` and
  `MachineConnection.SupportedProtocol` in the hub): change both together, and
  update ADR 0002.

### Agent detection

- Detection is evidence-based. When changing `src/detect/manifests/`, first
  capture the bottom-buffer state with
  `klark agent read <pane> --source detection --format text` (`--format ansi`
  when styling or alternate-screen behavior matters). Decide which visible
  controls are invariant and which are alternatives, and encode them as
  explicit AND/OR gates. Do not match whole-pane incidental text, and do not
  use the user-visible viewport for status, because users can scroll it.
- Iterate with the manifest hot-reload loop:
  1. Use the `klark-throwaway-repro` skill (`.agents/skills/`) to drive the
     real agent into the target state.
  2. Inspect with `klark agent explain <pane> --json`.
  3. Edit `src/detect/manifests/<agent>.toml`, then copy it to
     `~/.config/klark/agent-detection/<agent>.toml`. Never overwrite an
     existing override without asking.
  4. Run `klark server reload-agent-manifests`.
  5. When done, remove the temporary override.
- Unit-test the detection engine with synthetic manifests: parsing, regions,
  gates, priority, reload. Don't freeze a specific CLI's screens or rule IDs in
  tests. Check agent-specific behavior with live smoke tests and note the CLI
  version.
- There is no remote catalog any more (ADR 0001 D1). The bundled manifests are
  the only source.

### Vendored libghostty-vt

- `vendor/libghostty-vt.vendor.json` records the upstream commit currently
  vendored.
- Local patches live in `vendor/patches/libghostty-vt/` and are indexed in
  `vendor/libghostty-vt.patches.md`. Each index entry says why the patch
  exists, the files it touches, how it was verified, and when it can be
  removed.
- When updating libghostty-vt, recheck every patch. Drop the ones upstream
  fixed and reapply the rest.

### Rust conventions

- No `unwrap()` in production code. Use `tracing` for logging. Use `#[allow]`
  only with a comment explaining why.
- Gate platform code at compile time (`#[cfg(windows)]`, `#[cfg(unix)]`). Use
  `cfg!(...)` only for pure policy constants whose branches compile everywhere.
- Don't add dependencies without a reason; check existing ones first.
  `klark-link` deliberately has a small dependency set (tungstenite, flate2,
  serde, base64).
- Integration asset versions (`KLARK_INTEGRATION_VERSION` and matching
  constants) are migration versions: bump them once per change of the asset.
- Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState`
  or `Workspace` behavior must be testable with `AppState::test_new()` and
  `Workspace::test_new()`, without PTYs.
- For refactors touching identity, persisted state, protocol IDs,
  restore/handoff or detection authority: add characterization tests first,
  and use `AppState::assert_invariants_for_test()` /
  `Workspace::assert_invariants_for_test()` with the adversarial-state helpers.

## Hub (C#, `hub/`)

- Blazor Server with per-page `@rendermode InteractiveServer`. Sign-in uses a
  plain HTML form posting to `/account/login`, which is not interactive.
- **Every permission check runs on the server**, through `HubAccess`
  (`CanSee`, `CanType`). Hiding a button is never the only guard.
- **Hot path:** `MachineConnection.ReceiveLoopAsync` → `HandleBinaryFrameAsync`
  → `Dispatch`.
  - No awaiting viewers there: each viewer has its own bounded queue
    (`PaneViewer`).
  - No disk there: SQLite writes go through `HubStore`'s channel.
- **Lazy everywhere.** Pages pull what they show (`EnsureTreeAsync`,
  `RefreshTreesAsync`, `CallAsync`) and only while they are open. Don't add
  background polling that runs with no viewer.
- Config lives in the `Hub` section of `appsettings*.json`, bound to
  `HubOptions`. Secrets are stored as hashes (`dotnet run -- hash-password`,
  `hash-token`), never in clear text, except in the mock directory, which is
  for development only.
- Keep the UI usable at 375 px wide (phones).

## Testing

There is no CI. Run the relevant checks yourself before committing, and say
in the commit or PR what you ran.

```bash
cargo test -p klark-link                 # link: unit + end-to-end (no Zig needed)
just test                                # klark core (needs Zig 0.16: ZIG=/path/to/zig)
just check                               # formatting + tests
cd hub && dotnet build                   # hub (needs .NET 9 SDK), or:
cd hub && docker compose up -d --build   # http://localhost:8080, admin/admin (MockLdap)
python3 experiments/drive.py             # load matrix against the stand-in hub
```

When testing a debug klark build from inside a running klark session, clear
the inherited socket overrides so it talks to the debug `klark-dev` server:

```bash
env -u KLARK_SOCKET_PATH -u KLARK_CLIENT_SOCKET_PATH cargo run -- <command>
```

If you could not run something (missing toolchain, blocked network), say so
plainly and add or keep the matching item in `docs/STATUS.md`.

## Docs

- `docs/STATUS.md` is the single backlog. When you finish an item, move it to
  Done and say how it was verified ("unit test", "not compiled", …). New work
  you discover gets a new S-number.
- A decision that changes or adds architecture gets an ADR in `docs/adr/`
  (status, context, decision, consequences). Supersede old ADRs instead of
  rewriting them.
- A measurement gets an experiment in `docs/experiments/`: question, setup,
  numbers, conclusion. Say what it ran against (stand-in hub or real hub).
- `skills/klark/SKILL.md` is the agent skill for driving klark. Keep it in
  step with CLI changes.
- Put scratch notes and plans under `.local/` (ignored).

## Commits

- Short, descriptive subject lines; a body for anything non-trivial that
  explains what changed and why, with the STATUS IDs it touches (`S-05`).
- Commit on a branch and open a pull request into `master`. Don't push to
  `master` directly.
- Never commit secrets: tokens, passwords, `appsettings.Development.json` or
  `appsettings.Production.json` (both are ignored).
