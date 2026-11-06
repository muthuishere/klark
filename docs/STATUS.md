# klark status

What exists, what has been verified, and what is left. IDs are referenced from
the ADRs and experiments. Last updated 2026-11-06 (first real builds; messaging, ADR 0009).

## Done
| id | item | verified how |
|---|---|---|
| D-01 | Rename herdr → klark everywhere (code, env vars, paths, files); legacy config dir is moved over on first run | text rename only; **klark not compiled yet** (S-01) |
| D-02 | Self-update, announcements and remote catalog disabled | code review; same caveat |
| D-03 | Website, versioned docs, install scripts, nix and release scripts deleted | git |
| D-04 | `crates/klark-link`: Rust port of expose's upstream layer and the hub link | 12 unit and end-to-end tests pass |
| D-05 | Lazy pull protocol v1 (ADR 0002) | end-to-end test + E-01 |
| D-06 | Binary output frames, 50 ms merging (E-02, E-03) | tests + experiments |
| D-07 | `klark expose` subcommand | **not compiled** (S-01) |
| D-08 | Blazor Server hub: sign-in, machines page, live pane view, reply/Esc/Ctrl-C | **not compiled** (S-02) |
| D-09 | SQLite store, 10 MB per machine (ADR 0003) | **not compiled** |
| D-10 | Mock LDAP, LDAP and local sign-in, allowed groups (ADR 0004) | **not compiled** |
| D-11 | Fleet tokens + `klark-loadtest` | load test run against a stand-in hub |
| D-12 | Dockerfile, compose with `/data` volume, `ldap` profile | **not built** |
| D-13 | S-05 per-group machine visibility (`Machines[].Groups`, `AdminGroups`) | **not compiled** |
| D-14 | S-06 read-only viewers (`ReadOnlyGroups`, enforced server-side; mock user `viewer`) | **not compiled** |
| D-15 | S-07 offline machines: last pane list + recorded output from SQLite, loaded on expand | **not compiled** |
| D-16 | S-09 transcript view (`agent.read`, falls back to `pane.read`), pulled on request | **not compiled** |
| D-17 | S-10 hub closes links that speak another protocol version | **not compiled** |
| D-18 | S-11 sign-in rate limit (10/min per address) | **not compiled** |
| D-19 | S-13 per-viewer bounded queues; a lagging viewer is repainted from the current screen | **not compiled** |
| D-20 | S-14 reconnect backoff with ±50 % jitter | unit test |
| D-22 | S-22/S-23 git hooks, commit-format checker, changelog tooling, release perf scripts and justfile release/preview/docs recipes removed; Windows packaging, libghostty-vt tooling and render benchmarks kept | 62 maintenance tests pass |
| D-23 | Windows ConPTY: vendored portable-pty patches now read `KLARK_WINDOWS_CONPTY` and `klark-conpty.json`, matching the packaging script (the earlier rename had skipped `vendor/`, which would have made Windows reject its own bundle) | patch reverse-apply test + loader test pass; Windows not run |
| D-24 | "share this" for klark (idea from herdr-expose): `klark expose url` prints the hub link for the current pane; hub `/go` resolves the machine by reported name; `skills/klark` (ADR 0008; was `skills/klark-share`, merged) | `pane_url` unit test; `klark` and hub not compiled |
| D-25 | S-01 first `cargo build --release` of klark (Zig 0.16, macOS); rename fallout fixed in tests (checksum fixture, endpoint shape hashes, announcements test, SIGPIPE leak in unit tests) | `just test` (nextest): 3946 passed |
| D-26 | S-02 first hub build: .NET 10, Razor fix, SQLite advisory bump, per-machine store index bug (events table was never created), DataProtection keys kept in `/data/keys`, `.dockerignore` | `dotnet build` 0 warnings; `docker build` + `/healthz` + `/api/token` in the container |
| D-27 | Sign-in: Ldap default (direct / search-then-bind, LDAPS, StartTLS, memberOf); MockLdap opt-in in the same image; `KLARK_` env prefix for hub settings | build + MockLdap runs; **real LDAP not yet tried** |
| D-28 | User JWTs: `/token` page after Blazor sign-in, `POST /api/token` | curl |
| D-29 | S-25 agents message agents: `klark msg` (whoami, ls, send, reply, deliver, log, name, login, logout, status), link protocol v2, hub `/api/agents`, `/api/messages`, `/messages` page, `skills/klark` (ADR 0009; was `skills/klark-msg`, merged) | `tests/msg_sessions.rs` (two sessions); manual two-machine run through the real hub with machine token and user JWT |
| D-30 | `klark expose --agents <name>[,<name>...]` (repeatable `--agent`, env `KLARK_EXPOSE_AGENTS`): expose only panes running those agents (name, `session/name`, or type; a name beats a type; case-insensitive), re-evaluated on every pull; hidden panes/agents are refused for subscribe/input/keys/call/transcript/asset/deliver, session-wide calls filtered; `klark expose url` errors for an unexposed pane. `skills/klark-msg` and `skills/klark-share` merged into `skills/klark` | `crates/klark-link/src/filter.rs` unit tests; `tests/expose_sessions.rs` (two fake agents in two sessions) |
| D-21 | S-20 AGENTS.md and CONTRIBUTING.md rewritten for klark; herdr-only release skill and translation template removed; repo links point to muthuishere/klark | review |

## Not done: must do before relying on it
| id | item | notes |
|---|---|---|
| S-03 | Manual browser check: klark → hub → browser → reply lands in the pane | messaging path proven by script; the browser pane view not yet clicked through |
| S-28 | Try `Mode=Ldap` against a real directory (the compose `ldap` profile, or the company AD) | |
| S-29 | Release-build integration tests hardcode the `klark-dev` config dir (5 `live_handoff`, 1 `api_ping`); upstream did the same with `herdr-dev` | `just test` (debug) is green |
| S-04 | Delete the 10 old `.github/workflows/*` files in the GitHub UI (ADR 0007) | this agent cannot touch that folder |

## Not done: features
| id | item | notes |
|---|---|---|
| S-08 | Admin page: machine tokens and users in hub.db | replaces appsettings editing |
| S-12 | Measure Blazor circuits with 50+ viewers | **measured at 10 live viewers + 40 chat circuits on one hub (E-06)**: 1.2–13 MiB per circuit, ~0 idle CPU; 100 live viewers extrapolated, not run (one headless tab tops out ~20 circuits). Follow-ups from E-06: Dockerfile `--no-restore` breaks blazor.web.js (bug 1), dead tabs poll 3 min (bug 2), `/api/agents` fan-out unshared, global `Changed` re-renders every pane view |
| S-15 | Switch the tree source from `session.snapshot` to klark `events.subscribe` (cheaper on the machine) | |
| S-16 | Windows support for `klark expose` (named pipe) | |
| S-17 | Remove dead updater code in `src/update.rs` | disabled, still compiled |

| S-24 | Hub guest links: one pane, time-boxed, pairing code / QR, no account needed (idea from herdr-expose `share`) | ADR 0008 |
| S-26 | Hub admin: disconnect a machine, revoke its token (idea from herdr-expose `panic`) | with S-08 |
| S-27 | `klark expose` as an autostart service (systemd user unit, launchd agent) + `klark expose doctor` | |
| S-21 | Hub: notify (browser notification or sound) when a visible pane turns `blocked` | part of S-19 |
| S-19 | Pane search, filters and "blocked first" notifications (sound or push) in the hub | |

## Experiments
See [experiments/README.md](experiments/README.md): E-01 to E-04 are done,
E-05 to E-10 are to do.

## Open questions for the owner
- ~~O3~~: answered by S-05: per-group visibility, admins see all.
- ~~O4~~: no local UI in klark; the hub is its web UI. herdr-expose stays a separate product (ADR 0008).
- Should hub output history be kept longer than "while viewed", for example
  always recording blocked panes? That would break pure lazy pull, so it would
  need a decision.
