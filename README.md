# klark

klark is a terminal workspace for AI coding agents, with a built-in link to a
shared web hub.

- **Persistent panes.** Agents keep running in a background server when you
  detach. Run `klark` to reattach, and `ctrl+b q` to detach.
- **Agent-aware.** Every pane is marked working, blocked or idle, so you can
  see which agent is waiting for an answer.
- **Hub link.** `klark link` connects this machine to a [klark hub](hub/README.md),
  a Blazor Server app where a team signs in and watches and answers the agents
  on every linked machine (built for 100+ machines). Panes stream only while
  someone is looking at them.
- **No phoning home.** No self-update, no hosted announcements, no remote
  catalogs. klark uses what is built into the binary.

## Build

```bash
cargo build --release          # needs Zig 0.16 for the vendored libghostty-vt
./target/release/klark
```

## Link a machine to a hub

```bash
klark link --hub ws://hub-host:8080/agent --token <machine token> [--name laptop]
```

## Layout

| path | what |
|---|---|
| `src/` | klark itself (Rust) |
| `crates/klark-link` | the machine side of the hub link |
| `hub/` | the hub: C# Blazor Server, SQLite, Docker |
| `api/klark-api.schema.json` | socket API schema (`klark api schema`) |
| `docs/` | [status](docs/STATUS.md), [ADRs](docs/README.md), [experiments](docs/experiments/README.md) |
| `experiments/` | scripts behind the measured experiments |

Existing users: on first run, klark moves an old `~/.config/herdr` directory to
`~/.config/klark`. Environment variables are now `KLARK_*` (for example
`KLARK_SOCKET_PATH`).

klark is based on [herdr](https://github.com/herdrdev/herdr) (Apache-2.0).
